//! Cards: the card view of blocks of type `card`.
//!
//! A card has no storage of its own. It is created, read and patched as its block, so
//! every change is announced the way block changes are.

use fvoci_model::{Block, BlockType, Card, CardError, CardEvent, CardPatch, new_id};
use fvoci_store::{BlockFilter, ByAgent, Previews};

use super::list_page::ListPage;

use super::{App, ChangeNotification};
use crate::auth::now_millis;
use crate::error::AppError;

/// Whether a list of cards gives each card its preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardPreviews {
    /// It does not.
    Without,
    /// Each card has the file of its body's first image and the beginning of its
    /// plain text.
    With,
}

impl From<CardPreviews> for Previews {
    fn from(previews: CardPreviews) -> Self {
        match previews {
            CardPreviews::Without => Self::Without,
            CardPreviews::With => Self::With,
        }
    }
}

impl App {
    /// Creates a card on a board and returns it as stored; listeners are told as for
    /// any inserted block.
    ///
    /// The card gets a new identifier, `board_id` as its board, `user_id` as its
    /// creator and modifier and the current time as creation and modification time,
    /// whatever `card` says about them.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board does not exist and
    /// a bad request if the title or the properties are too large, both
    /// with the message prefixed by `cannot create card`.
    pub(in crate::app) async fn create_card(
        &self,
        card: Card,
        board_id: &str,
        user_id: &str,
        notification: ChangeNotification,
    ) -> Result<Card, AppError> {
        let now = now_millis();
        let card = Card {
            id: new_id(),
            board_id: board_id.to_owned(),
            created_by: user_id.to_owned(),
            modified_by: user_id.to_owned(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            ..card
        };

        let mut blocks = [Block::from(card)];
        self.insert_blocks(&mut blocks, user_id, notification)
            .await
            .map_err(|error| error.context("cannot create card"))?;
        let [block] = blocks;
        Ok(Card::from_block(&block, now)?)
    }

    /// Returns one page of the cards of a board ([`ListPage`]), in no particular order.
    /// With [`CardPreviews::With`] each card has its preview.
    ///
    /// # Errors
    ///
    /// Returns an internal error if one of the board's card blocks cannot be read
    /// as a card, because a field of it has the wrong type.
    pub async fn cards_for_board(
        &self,
        board_id: &str,
        page: ListPage,
        previews: CardPreviews,
    ) -> Result<Vec<Card>, AppError> {
        let mut filter = BlockFilter::container(board_id).with_type(BlockType::Card);
        filter.paging = Some(page.paging());
        let blocks = self
            .store()
            .get_listed_blocks(&filter, previews.into())
            .await?;

        let now = now_millis();
        // The fields of a card that are not of their kind are left out
        // (`card_from_stored_block`), so no stored card fails the list; an error here
        // would be the server's. A list says whether a card has a body; the body is
        // read with the card.
        blocks
            .iter()
            .map(|listed| Self::card_of_listed(listed, now))
            .collect()
    }

    /// Applies a patch to a card and returns the patched card; listeners are told as
    /// for any patched block.
    ///
    /// The property values are merged into the stored ones in the store's transaction
    /// ([`fvoci_model::BlockPatch::property_changes`]).
    ///
    /// # Errors
    ///
    /// Returns a bad request if the patch is not valid (an icon of more than one
    /// grapheme), an internal error if the patched block is no card, and otherwise the
    /// errors of `App::patch_block_as` with the message prefixed by `cannot patch
    /// card {id}`; written by `user_id`, or by an agent through a token of theirs.
    pub(in crate::app) async fn patch_card(
        &self,
        patch: &CardPatch,
        card_id: &str,
        user_id: &str,
        notification: ChangeNotification,
        agent: Option<ByAgent<'_>>,
    ) -> Result<Card, AppError> {
        // An invalid icon is a bad request, matching new card validation.
        let block_patch = patch.to_block_patch()?;
        let block = self
            .patch_block_as(card_id, &block_patch, user_id, notification, agent)
            .await
            .map_err(|error| error.context(format_args!("cannot patch card {card_id}")))?;
        self.with_last_comment_at(card_from_stored_block(&block, now_millis())?)
            .await
    }

    /// Returns the card with the given identifier.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no block with this identifier and
    /// an internal error if the block is not a card or cannot be read as one.
    pub async fn card(&self, card_id: &str) -> Result<Card, AppError> {
        let block = self.block(card_id).await?;
        self.card_of_block(&block).await
    }

    /// One page of the activity of the card `card_id`, the newest first
    /// ([`fvoci_model::CardEvent`]).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the lines cannot be read.
    pub async fn card_activity(
        &self,
        card_id: &str,
        page: ListPage,
    ) -> Result<Vec<CardEvent>, AppError> {
        Ok(self.store().card_events(card_id, page.paging()).await?)
    }

    /// Returns the block of the card of the board that has `number`, if there is one.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the card cannot be looked for.
    pub async fn card_by_number(
        &self,
        board_id: &str,
        number: u32,
    ) -> Result<Option<Block>, AppError> {
        Ok(self.store().get_card_by_number(board_id, number).await?)
    }

    /// Returns the card of a card block that was read before, with when its last
    /// comment was posted.
    ///
    /// # Errors
    ///
    /// Returns an internal error if the block cannot be read as a card, and the store's
    /// error if its comments cannot be read.
    pub async fn card_of_block(&self, block: &Block) -> Result<Card, AppError> {
        self.with_last_comment_at(card_from_stored_block(block, now_millis())?)
            .await
    }

    /// The card with when its last comment was posted.
    async fn with_last_comment_at(&self, card: Card) -> Result<Card, AppError> {
        let last_comment_at = self.store().get_last_comment_at(&card.id).await?;
        Ok(Card {
            last_comment_at,
            ..card
        })
    }
}

/// Builds the card view of a stored block.
///
/// A card cannot be stored with `properties` that are no object, or an `icon` or
/// `isTemplate` of another kind, any more, but one stored before, or written straight to
/// the database, may still hold such a field. Instead of failing the entire card list,
/// malformed fields are dropped ([`Block::drop_malformed_fields`]) and a warning is logged.
pub(super) fn card_from_stored_block(block: &Block, now: i64) -> Result<Card, CardError> {
    let mut readable = block.clone();
    let malformed = readable.drop_malformed_fields();
    if !malformed.is_empty() {
        tracing::warn!(
            block_id = %block.id,
            board_id = %block.container_id,
            fields = ?malformed,
            "fields of a card are not of their kind; giving the card out without them"
        );
    }
    Card::from_block(&readable, now)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// The API and the store refuse to write such a block, so this is the one place
    /// that can still show what reading one does.
    #[test]
    fn a_stored_card_whose_properties_are_no_object_is_read_without_properties() {
        use serde_json::json;

        let card_block = |properties: Value| {
            let mut block = Block {
                id: "c-card".to_owned(),
                container_id: "b-board".to_owned(),
                block_type: fvoci_model::BlockType::Card,
                title: "a card".to_owned(),
                created_at: 1,
                updated_at: 1,
                ..Block::default()
            };
            block.fields.insert("properties".to_owned(), properties);
            block.fields.insert("icon".to_owned(), json!("x"));
            block
        };

        let fine = card_from_stored_block(&card_block(json!({"p1": "v1"})), 5)
            .expect("a well-formed card is read");
        assert_eq!(
            fine.properties.get("p1"),
            Some(&json!("v1")),
            "a well-formed card keeps its properties"
        );

        for properties in [json!("a string"), json!([1, 2]), json!(42), json!(null)] {
            let card = card_from_stored_block(&card_block(properties.clone()), 5)
                .expect("the malformed card is read all the same");
            assert!(
                card.properties.is_empty(),
                "without properties: {properties}"
            );
            assert_eq!(card.title, "a card", "the rest of the card: {properties}");
            assert_eq!(card.icon, "x", "the rest of the card: {properties}");
        }
    }

    /// A card stored before `icon` and `isTemplate` were checked is read without the
    /// field that is not of its kind.
    #[test]
    fn a_stored_card_whose_icon_or_template_flag_is_malformed_is_read_without_it() {
        use serde_json::json;

        for (field, value) in [
            ("icon", json!(5)),
            ("icon", json!({"x": 1})),
            ("isTemplate", json!("yes")),
        ] {
            let mut block = Block {
                id: "c-card".to_owned(),
                container_id: "b-board".to_owned(),
                block_type: fvoci_model::BlockType::Card,
                title: "a card".to_owned(),
                created_at: 1,
                updated_at: 1,
                ..Block::default()
            };
            block.fields.insert(field.to_owned(), value.clone());
            block
                .fields
                .insert("properties".to_owned(), json!({"p1": "v1"}));
            let card = card_from_stored_block(&block, 5)
                .unwrap_or_else(|error| panic!("{field} {value}: {error}"));
            assert_eq!(card.icon, "", "{field} {value}");
            assert!(!card.is_template, "{field} {value}");
            assert_eq!(
                card.properties.len(),
                1,
                "the rest is kept: {field} {value}"
            );
        }
    }
}
