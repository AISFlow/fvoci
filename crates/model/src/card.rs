//! Cards: a typed view over blocks of type `card`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;

use crate::JsonMap;
use crate::block::{Block, BlockPatch, BlockType, PropertyChanges};
use crate::body::{Body, BodyChange, BodySummary};
use crate::body_markdown::ComplexBlock;
use crate::card_property::{PersonProperties, check_card_property_values};
use crate::id::new_id;
use crate::refusal::{RefusalReason, Refusals};

/// The member of a card's fields that holds its icon.
pub(crate) const CARD_ICON_FIELD: &str = "icon";

/// How deep a card hierarchy on a board may go: a top-level card is at depth 1,
/// its child at depth 2, and its grandchild at depth 3.
pub const MAX_CARD_DEPTH: u32 = 3;

/// Errors from validating cards and converting them to and from blocks.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CardError {
    /// The card has a missing or malformed member; the words say which.
    #[error("{0}")]
    Invalid(&'static str),
    /// The block to convert is not of type `card`.
    #[error("cannot convert block to card: not a card block")]
    NotCardBlock,
    /// A field of the block has the wrong JSON type for a card.
    #[error("invalid type for field '{field}'")]
    InvalidFieldType {
        /// The name of the offending field.
        field: &'static str,
    },
    /// The card names another board than the request's path does.
    #[error("the card names another board than the path")]
    BoardIdMismatch,
    /// A new body is sent without the time the card was read at.
    #[error("a new body is sent with readAt, the updatedAt of the card as it was read")]
    ReadAtMissing,
}

/// A card: a titled group of properties with a body on a board.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Card {
    /// The card's identifier.
    pub id: String,
    /// The identifier of the board the card belongs to.
    pub board_id: String,
    /// The identifier of the parent card on the same board; empty for a top-level card
    /// (depth 1). A card template has no parent.
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub parent_id: String,
    /// The card's number on its board, as `#12` names it: from 1, one more for each card
    /// made on the board, never given again, also once the card is gone. A card keeps it
    /// while it is one; a copy and an import are new cards with new numbers, and a card
    /// that comes back from the trash has its own. `null` for a template of the board's
    /// cards (`isTemplate`): a card made a template hides its number while keeping it
    /// stored, and regains its old number without consuming a new one if made a card
    /// again; a template created without a number takes the next number when made a card.
    /// The server gives it: what a request sends here is ignored, whatever it is.
    #[schema(minimum = 1)]
    #[serde(deserialize_with = "crate::block::number_as_sent")]
    pub number: Option<u32>,
    /// The identifier of the user who created the card.
    pub created_by: String,
    /// The identifier of the user who last modified the card.
    pub modified_by: String,
    /// The display title.
    pub title: String,
    /// The card's icon; at most one grapheme.
    pub icon: String,
    /// Whether the card is a template.
    pub is_template: bool,
    /// Property values keyed by property identifier: a string, a list of strings, or
    /// `null`; for a property of the type `date` a date value (`{"from":
    /// "2026-03-05"}`, `{"from": "2026-03-05", "to": "2026-03-09"}` or `{"from":
    /// 1772703000000}`) or `null`; and for a property of the type `relation` a list of
    /// relation links (`{"card": "<cardId>", "kind": "fs" | "ss" | "ff" | "sf",
    /// "lagDays": -3650..=3650}`, normalized with all three fields when stored) or
    /// `null`.
    #[schema(value_type = Object)]
    pub properties: JsonMap,
    /// The card's body. `null` for a card without one, and in a list of cards, which
    /// says only whether there is one (`hasBody`): a card is read alone for its body.
    pub body: Option<Body>,
    /// Whether the card has a body.
    pub has_body: bool,
    /// How many task items the body has.
    pub tasks_total: u32,
    /// How many task items of the body are checked.
    pub tasks_done: u32,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// Deletion time in milliseconds since the Unix epoch; absent while not deleted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::positive_millis"
    )]
    #[schema(nullable = false)]
    pub deleted_at: Option<i64>,
    /// When the card's last comment was posted (the `createdAt` of the newest of its
    /// comments, the blocks of the type `comment` directly on it), in milliseconds since
    /// the Unix epoch; zero if it has none. Changing a comment does not change it; a
    /// deleted comment does not count, and counts again with its time if restored. A
    /// comment does not change the card's `updatedAt`. A new comment comes as
    /// `blockChanged`, so a client can keep the newer time; after a deletion the card is
    /// read again for it. A number sent here in a request is ignored.
    pub last_comment_at: i64,
    /// What a gallery shows of the card's body: only in a list asked for it
    /// (`getCards` with `preview=true`), and left out otherwise. A request need not send
    /// it; what it sends is not stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<CardPreview>,
    /// The body in Markdown, as `getCard` with `format=markdown` gives it in place of
    /// `body`, left out otherwise: `bodyMarkdown` of `patchCard` writes it back, and
    /// `appendCardMarkdown` adds to it. Files of the board are named
    /// `fvoci-file://{fileId}` (an image `![alt](fvoci-file://…)`, a file
    /// `[name](fvoci-file://…)`), people `@[name](fvoci-user://{userId})`, cards and
    /// pages `[[id]]`; a formula is `$…$`, one of its own block `$$…$$`. A `[`, `]`,
    /// `$` or any other mark Markdown would read is written after a backslash in text.
    /// An empty paragraph is not written.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub body_markdown: Option<String>,
    /// With `bodyMarkdown`: whether the body has blocks its Markdown does not hold as
    /// they are (`complexBlocks`), which `bodyMarkdown` may not be written over (409
    /// `complex_blocks_use_append`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub has_complex_blocks: Option<bool>,
    /// With `bodyMarkdown`: the blocks it does not hold as they are, in the order of
    /// the body.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub complex_blocks: Option<Vec<ComplexBlock>>,
}

/// A card in short, as a list gives it with `compact=true` (`getCards`): enough to tell
/// the cards apart and choose one to read.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompactCard {
    /// The card's identifier.
    pub id: String,
    /// The card's number on its board (`Card.number`).
    #[schema(minimum = 1)]
    pub number: Option<u32>,
    /// The identifier of the parent card; empty for a top-level card.
    pub parent_id: String,
    /// The display title.
    pub title: String,
    /// Property values keyed by property identifier, as `Card.properties`.
    #[schema(value_type = Object)]
    pub properties: JsonMap,
    /// Whether the card has a body.
    pub has_body: bool,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

impl From<Card> for CompactCard {
    fn from(card: Card) -> Self {
        Self {
            id: card.id,
            number: card.number,
            parent_id: card.parent_id,
            title: card.title,
            properties: card.properties,
            has_body: card.has_body,
            updated_at: card.updated_at,
        }
    }
}

/// What a gallery shows of a card's body.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct CardPreview {
    /// The file the body's first image shows (`getFile` gives it, and `getSharedFile` to
    /// the holder of the board's link); empty if the body has no image.
    pub image_file_id: String,
    /// The beginning of the body's text: its plain text as it is searched (a line per
    /// paragraph, heading, code block and formula block, separated by `\n`; a formula
    /// gives its source, an image and a mention nothing), at most 200 UTF-16 code units,
    /// cut between characters as they are seen (a flag or a letter with its accent stays
    /// whole), without white space at its end. Nothing says it was cut. Empty for a card
    /// without text.
    pub text: String,
}

/// The longest text of a [`CardPreview`], in UTF-16 code units.
pub const CARD_PREVIEW_TEXT_LENGTH: usize = 200;

impl CardPreview {
    /// The preview of a body whose plain text is `text` and whose first image shows
    /// `image_file_id` (empty for none).
    #[must_use]
    pub fn new(image_file_id: String, text: &str) -> Self {
        let mut units = 0;
        let end = text
            .grapheme_indices(true)
            .find(|&(_, grapheme)| {
                units += grapheme.encode_utf16().count();
                units > CARD_PREVIEW_TEXT_LENGTH
            })
            .map_or(text.len(), |(at, _)| at);
        Self {
            image_file_id,
            text: text[..end].trim_end().to_owned(),
        }
    }

    /// The preview of `body`, as a list asked for previews gives it of the stored body.
    #[must_use]
    pub fn of(body: &Body) -> Self {
        let image_file_id = body.first_image_file_id().unwrap_or_default().to_owned();
        Self::new(image_file_id, &body.text())
    }
}

impl Card {
    /// Empties the users the card names, as a block's are for whoever reads with a read
    /// token alone ([`Block::hide_people`]): its creator and last writer, the users of
    /// its person properties (by `people`, the board's), and the mentions of its body.
    pub fn hide_people(&mut self, people: &PersonProperties) {
        self.created_by.clear();
        self.modified_by.clear();
        people.hide_values(&mut self.properties);
        if let Some(body) = self.body.as_mut() {
            body.clear_mentions();
        }
    }

    /// The card as a list shows it: without its body, with what the list says about
    /// the body instead.
    #[must_use]
    pub fn listed(self, summary: BodySummary) -> Self {
        Self {
            body: None,
            has_body: summary.has_body,
            tasks_total: summary.tasks.total,
            tasks_done: summary.tasks.done,
            ..self
        }
    }

    /// Fills in the members a new card may leave out: a fresh identifier, and `now`
    /// (milliseconds since the epoch) for a zero creation or modification time.
    pub fn populate(&mut self, now: i64) {
        if self.id.is_empty() {
            self.id = new_id();
        }
        if self.created_at == 0 {
            self.created_at = now;
        }
        if self.updated_at == 0 {
            self.updated_at = now;
        }
    }

    /// Checks the shape of the card as a request to make it sends it, all of it.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: an icon of more than one grapheme, and
    /// each value of `properties` that has not the shape of one.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        refuse_icon_and_values(
            &mut refused,
            Some(&self.icon),
            &self.properties,
            "properties",
        );
        refused.into_result()
    }

    /// Checks that the card is complete.
    ///
    /// # Errors
    ///
    /// Returns [`CardError::Invalid`] naming the first problem: a missing identifier, board
    /// or timestamp, or an icon of more than one grapheme.
    pub fn validate(&self) -> Result<(), CardError> {
        if self.id.is_empty() {
            return Err(CardError::Invalid(NO_ID));
        }
        if self.board_id.is_empty() {
            return Err(CardError::Invalid(NO_BOARD));
        }
        check_icon(&self.icon)?;
        if self.created_at == 0 {
            return Err(CardError::Invalid(NO_CREATED_AT));
        }
        if self.updated_at == 0 {
            return Err(CardError::Invalid(NO_UPDATED_AT));
        }
        Ok(())
    }

    /// Builds a card from a block of type `card`.
    ///
    /// Missing fields take their empty value, and a zero timestamp becomes `now`. Its
    /// `last_comment_at` is zero: a block does not know its comments, and whoever gives
    /// the card out reads them (the app's `card_of_block`, the card list).
    ///
    /// # Errors
    ///
    /// Returns [`CardError::NotCardBlock`] for any other block type, and
    /// [`CardError::InvalidFieldType`] if `icon` is not a string, `isTemplate` is not a
    /// boolean or `properties` is not an object (`null` as `icon` or `isTemplate` is
    /// taken as absent). A card's content is its body; a
    /// `contentOrder` that a card stored before has is left among the block's fields and
    /// is not read.
    pub fn from_block(block: &Block, now: i64) -> Result<Self, CardError> {
        if block.block_type != BlockType::Card {
            return Err(CardError::NotCardBlock);
        }

        let icon = match block.fields.get(CARD_ICON_FIELD) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(icon)) => icon.clone(),
            Some(_) => return Err(CardError::InvalidFieldType { field: "icon" }),
        };
        let is_template = match block.fields.get("isTemplate") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(is_template)) => *is_template,
            Some(_) => {
                return Err(CardError::InvalidFieldType {
                    field: "isTemplate",
                });
            }
        };
        let properties = match block.fields.get("properties") {
            None => JsonMap::new(),
            Some(Value::Object(properties)) => properties.clone(),
            Some(_) => {
                return Err(CardError::InvalidFieldType {
                    field: "properties",
                });
            }
        };

        let summary = block.body.as_ref().map(Body::summary).unwrap_or_default();
        let mut card = Self {
            id: block.id.clone(),
            board_id: block.container_id.clone(),
            parent_id: block.parent_id.clone(),
            number: block.shown_number(),
            created_by: block.created_by.clone(),
            modified_by: block.modified_by.clone(),
            title: block.title.clone(),
            icon,
            is_template,
            properties,
            body: block.body.clone(),
            has_body: summary.has_body,
            tasks_total: summary.tasks.total,
            tasks_done: summary.tasks.done,
            created_at: block.created_at,
            updated_at: block.updated_at,
            deleted_at: block.deleted_at,
            // A block does not know its comments.
            last_comment_at: 0,
            preview: None,
            body_markdown: None,
            has_complex_blocks: None,
            complex_blocks: None,
        };
        card.populate(now);
        Ok(card)
    }
}

impl From<Card> for Block {
    /// Converts the card to a block of type `card` on the card's board, preserving its
    /// `parent_id`.
    fn from(card: Card) -> Self {
        let mut fields = JsonMap::new();
        fields.insert(CARD_ICON_FIELD.to_owned(), Value::from(card.icon));
        fields.insert("isTemplate".to_owned(), Value::from(card.is_template));
        fields.insert("properties".to_owned(), Value::Object(card.properties));

        Self {
            id: card.id,
            parent_id: card.parent_id,
            created_by: card.created_by,
            modified_by: card.modified_by,
            block_type: BlockType::Card,
            title: card.title,
            fields,
            body: card.body,
            created_at: card.created_at,
            updated_at: card.updated_at,
            deleted_at: card.deleted_at,
            container_id: card.board_id,
            number: card.number,
            edited_at: 0,
        }
    }
}

/// What a card is changed to. A member that is left out stays as it is.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct CardPatch {
    /// The new parent card identifier on the same board; `""` or `null` moves the card
    /// to the top level (depth 1). Left out, the parent stays as it is.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_parent_id_patch"
    )]
    pub parent_id: Option<String>,
    /// The new title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The new icon; at most one grapheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Property values to set, by property identifier, each replacing the card's value
    /// for that property. The card's other values stay as they are: two patches that
    /// set different properties of a card at about the same time both keep theirs, and
    /// of two that set the same property the later one wins.
    #[schema(value_type = Object)]
    pub updated_properties: JsonMap,
    /// Identifiers of properties whose value the card is to lose. A property named here
    /// and in `updatedProperties` loses its value.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deleted_properties: Vec<String>,
    /// The new body; `null` takes the body away. Sent only with `readAt`.
    #[serde(skip_serializing_if = "BodyChange::is_keep")]
    #[schema(value_type = Option<Body>)]
    pub body: BodyChange,
    /// The `updatedAt` of the card as the client read it: if the card has changed since,
    /// the patch is refused with 409 and changes nothing. Needed with `body`, and
    /// checked whenever it is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_at: Option<i64>,
    /// The new body in Markdown, as `getCard` with `format=markdown` gives it
    /// (`Card.bodyMarkdown`), in place of `body`; Markdown that says nothing takes the
    /// body away. Sent only with `readAt`, never with `body`. Refused with 409
    /// `complex_blocks_use_append` while the body has blocks its Markdown does not hold
    /// as they are (`Card.hasComplexBlocks`): `appendCardMarkdown` adds to it instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_markdown: Option<String>,
}

impl CardPatch {
    /// Checks the shape of the patch as a request sends it, all of it.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: an icon of more than one grapheme, a new
    /// body without `readAt`, and each value of `updatedProperties` that has not the
    /// shape of one. A value of a property the patch also deletes is not looked at: it
    /// is gone when the patch is applied.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if !self.body.is_keep() && self.read_at.is_none() {
            refused.refuse(
                "readAt",
                RefusalReason::Required,
                CardError::ReadAtMissing.to_string(),
            );
        }
        let kept = self
            .updated_properties
            .iter()
            .filter(|(id, _)| !self.deleted_properties.contains(id));
        refuse_icon_and_values(
            &mut refused,
            self.icon.as_deref(),
            kept,
            "updatedProperties",
        );
        refused.into_result()
    }

    /// Applies the patch to `card`: the updated properties are set, then the deleted ones
    /// removed.
    pub fn apply(&self, card: &mut Card) {
        if let Some(parent_id) = &self.parent_id {
            card.parent_id.clone_from(parent_id);
        }
        if let Some(title) = &self.title {
            card.title.clone_from(title);
        }
        if let Some(icon) = &self.icon {
            card.icon.clone_from(icon);
        }
        self.property_changes().apply_to(&mut card.properties);
        self.body.apply_to(&mut card.body);
    }

    /// The changes of the patch to the card's property values.
    fn property_changes(&self) -> PropertyChanges {
        PropertyChanges {
            updated: self.updated_properties.clone(),
            deleted: self.deleted_properties.clone(),
        }
    }

    /// Checks that the patch may be applied.
    ///
    /// # Errors
    ///
    /// Returns [`CardError::Invalid`] if the new icon is more than one grapheme, and
    /// [`CardError::ReadAtMissing`] for a new body without the time it was read at.
    pub fn validate(&self) -> Result<(), CardError> {
        if !self.body.is_keep() && self.read_at.is_none() {
            return Err(CardError::ReadAtMissing);
        }
        match &self.icon {
            Some(icon) => check_icon(icon),
            None => Ok(()),
        }
    }

    /// Converts the patch to the equivalent patch of the card's block. The property
    /// values go in [`BlockPatch::property_changes`], to be merged into the values the
    /// block has when it is patched.
    ///
    /// # Errors
    ///
    /// Returns the error from [`CardPatch::validate`] if the patch is not valid.
    pub fn to_block_patch(&self) -> Result<BlockPatch, CardError> {
        self.validate()?;

        let mut updated_fields = JsonMap::new();
        if let Some(icon) = &self.icon {
            updated_fields.insert(CARD_ICON_FIELD.to_owned(), Value::from(icon.clone()));
        }

        Ok(BlockPatch {
            parent_id: self.parent_id.clone(),
            title: self.title.clone(),
            updated_fields,
            property_changes: self.property_changes(),
            body: self.body.clone(),
            read_at: self.read_at,
            ..BlockPatch::default()
        })
    }
}

/// Deserializes a nullable string where `null` means the empty string.
fn deserialize_nullable_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// Deserializes a nullable `parentId` in a patch: absent leaves the parent unchanged
/// (`None`, via `#[serde(default)]`), while `null` or `""` moves the card to the top
/// level (`Some("")`), and a string moves it below that parent.
fn deserialize_parent_id_patch<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(
        Option::<String>::deserialize(deserializer)?.unwrap_or_default(),
    ))
}

/// What an icon of more than one grapheme cluster is refused with.
pub(crate) const ICON_OF_SEVERAL: &str = "an icon is at most one grapheme";

/// The words of [`CardError::Invalid`] for a card without an identifier.
const NO_ID: &str = "the card has no identifier";
/// The words of [`CardError::Invalid`] for a card without a board.
const NO_BOARD: &str = "the card is on no board";
/// The words of [`CardError::Invalid`] for a card without the time it was made.
const NO_CREATED_AT: &str = "the card has no createdAt";
/// The words of [`CardError::Invalid`] for a card without the time it was changed.
const NO_UPDATED_AT: &str = "the card has no updatedAt";

/// Whether the text may be an icon, of a card or of a page: at most one grapheme
/// cluster.
pub(crate) fn is_icon(icon: &str) -> bool {
    icon.graphemes(true).count() <= 1
}

/// Rejects an icon of more than one grapheme cluster.
fn check_icon(icon: &str) -> Result<(), CardError> {
    if !is_icon(icon) {
        return Err(CardError::Invalid(ICON_OF_SEVERAL));
    }
    Ok(())
}

/// Gathers what [`check_icon`] and the check of property values refuse of a request
/// whose values are its member `values_member`.
fn refuse_icon_and_values<'v>(
    refused: &mut Refusals,
    icon: Option<&str>,
    values: impl IntoIterator<Item = (&'v String, &'v Value)>,
    values_member: &str,
) {
    if icon.is_some_and(|icon| check_icon(icon).is_err()) {
        refused.refuse(
            "icon",
            RefusalReason::Invalid,
            CardError::Invalid(ICON_OF_SEVERAL).to_string(),
        );
    }
    if let Err(of_values) = check_card_property_values(values, values_member) {
        for refusal in of_values {
            refused.push(refusal);
        }
    }
}
