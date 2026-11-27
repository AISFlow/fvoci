//! The cards of a board that a list asks for by more than its page: by their title or
//! number, their place, the people they are given to, a value, their last change, and
//! in another order than the oldest first. Their blocks are read and chosen here, and
//! only those of the page asked for are made cards.

use fvoci_model::{Block, BlockType, Card, CardPropertyDefinition, assigned_users};
use fvoci_store::{BlockFilter, ListedBlock};
use serde_json::Value;

use super::App;
use super::cards::CardPreviews;
use super::list_page::ListPage;
use crate::auth::now_millis;
use crate::error::AppError;

/// Where the cards are on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardsUnder {
    /// The cards of the top level.
    Top,
    /// The cards right below this one.
    Card(String),
}

/// The order of the cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardsOrder {
    /// The oldest first.
    #[default]
    CreatedFirst,
    /// The last changed first.
    UpdatedLast,
    /// By their numbers; templates, which show none, last.
    Numbered,
}

/// Which cards of a board a list gives, and in what order.
#[derive(Debug, Clone, Default)]
pub struct CardsFilter {
    /// A text the title holds, whatever the case of its letters, or `#12`: the card of
    /// that number.
    pub text: Option<String>,
    /// Where the cards are.
    pub under: Option<CardsUnder>,
    /// A user a property that gives the card to people (`person`, `multiPerson`) holds.
    pub given_to: Option<String>,
    /// A property and a value it is, or holds among its values.
    pub value: Option<(String, String)>,
    /// Changed at or after this time, in milliseconds since the Unix epoch.
    pub updated_since: Option<i64>,
    /// Their order.
    pub order: CardsOrder,
}

impl CardsFilter {
    /// Whether the filter chooses every card in the order of the store's pages.
    #[must_use]
    pub fn is_everything(&self) -> bool {
        self.text.is_none()
            && self.under.is_none()
            && self.given_to.is_none()
            && self.value.is_none()
            && self.updated_since.is_none()
            && self.order == CardsOrder::CreatedFirst
    }
}

/// What a list asks of the titles, read once for all the cards: the card of a number
/// (`#12`), or a text the title holds whatever the case of its letters.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TitleQuery {
    Number(u32),
    Holds(String),
}

impl TitleQuery {
    fn of(text: &str) -> Self {
        match text.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()) {
            Some(number) => Self::Number(number),
            None => Self::Holds(text.to_lowercase()),
        }
    }

    fn chooses(&self, block: &Block) -> bool {
        match self {
            Self::Number(number) => block.shown_number() == Some(*number),
            Self::Holds(text) => block.title.to_lowercase().contains(text.as_str()),
        }
    }
}

impl CardsFilter {
    /// Whether the card of `block` is one the filter chooses, asked of the block as
    /// the list read it (as its card would say), `title` being what the filter asks of
    /// the titles and `definitions` the properties of its board.
    fn chooses(
        &self,
        block: &Block,
        title: Option<&TitleQuery>,
        definitions: &[CardPropertyDefinition],
    ) -> bool {
        // A card whose properties are no object has none, as its card says.
        let properties = block.fields.get("properties").and_then(Value::as_object);
        title.is_none_or(|title| title.chooses(block))
            && self.under.as_ref().is_none_or(|under| match under {
                CardsUnder::Top => block.parent_id.is_empty(),
                CardsUnder::Card(parent) => &block.parent_id == parent,
            })
            && self.given_to.as_deref().is_none_or(|user| {
                properties.is_some_and(|properties| {
                    assigned_users(properties, definitions).contains(&user)
                })
            })
            && self.value.as_ref().is_none_or(|(property, value)| {
                holds(
                    properties.and_then(|properties| properties.get(property)),
                    value,
                )
            })
            && self
                .updated_since
                .is_none_or(|since| block.updated_at >= since)
    }
}

impl CardsOrder {
    fn sort(self, listed: &mut [&ListedBlock]) {
        match self {
            Self::CreatedFirst => {
                listed.sort_by(|a, b| {
                    let (a, b) = (&a.block, &b.block);
                    (a.created_at, &a.id).cmp(&(b.created_at, &b.id))
                });
            }
            Self::UpdatedLast => {
                listed.sort_by(|a, b| {
                    let (a, b) = (&a.block, &b.block);
                    (b.updated_at, &b.id).cmp(&(a.updated_at, &a.id))
                });
            }
            Self::Numbered => {
                listed.sort_by_key(|listed| {
                    let number = listed.block.shown_number();
                    (number.is_none(), number, listed.block.created_at)
                });
            }
        }
    }
}

fn holds(value: Option<&Value>, wanted: &str) -> bool {
    match value {
        Some(Value::String(text)) => text == wanted,
        Some(Value::Array(items)) => items.iter().any(|item| item.as_str() == Some(wanted)),
        _ => false,
    }
}

impl App {
    /// A card of a list, from its block as the list read it.
    pub(super) fn card_of_listed(listed: &ListedBlock, now: i64) -> Result<Card, AppError> {
        let card = super::cards::card_from_stored_block(&listed.block, now)
            .map_err(|error| AppError::from(error).context("Block2Card fail"))?;
        Ok(Card {
            last_comment_at: listed.last_comment_at,
            preview: listed.preview.as_ref().map(|preview| {
                fvoci_model::CardPreview::new(
                    preview.first_image.clone().unwrap_or_default(),
                    &preview.text,
                )
            }),
            ..card.listed(listed.summary)
        })
    }

    /// One page of the cards of the board `board_id` that `filter` chooses, in its
    /// order. The cards below a card are asked of the store as such; the rest is
    /// chosen and ordered among the blocks it lists, and only the page made cards.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the cards or the board cannot be read.
    pub async fn filtered_cards(
        &self,
        board_id: &str,
        filter: &CardsFilter,
        page: ListPage,
        previews: CardPreviews,
    ) -> Result<Vec<Card>, AppError> {
        let mut filter_of_store = BlockFilter::container(board_id).with_type(BlockType::Card);
        if let Some(CardsUnder::Card(parent)) = &filter.under {
            if !fvoci_model::is_id(parent) {
                return Ok(Vec::new());
            }
            filter_of_store = filter_of_store.with_parent(parent);
        }
        let listed = self
            .store()
            .get_listed_blocks(&filter_of_store, previews.into())
            .await?;
        let definitions = match filter.given_to {
            Some(_) => self.store().get_container(board_id).await?.card_properties,
            None => Vec::new(),
        };
        let title = filter.text.as_deref().map(TitleQuery::of);
        let mut chosen: Vec<&ListedBlock> = listed
            .iter()
            .filter(|listed| filter.chooses(&listed.block, title.as_ref(), &definitions))
            .collect();
        filter.order.sort(&mut chosen);
        let paging = page.paging();
        let per_page = usize::try_from(paging.per_page.get()).unwrap_or(usize::MAX);
        let skipped = usize::try_from(paging.page)
            .unwrap_or(usize::MAX)
            .saturating_mul(per_page);
        let now = now_millis();
        chosen
            .into_iter()
            .skip(skipped)
            .take(per_page)
            .map(|listed| Self::card_of_listed(listed, now))
            .collect()
    }
}
