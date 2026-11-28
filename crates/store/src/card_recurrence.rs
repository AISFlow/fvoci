//! Cards that repeat ([`fvoci_model::Recurrence`]): the write of a card settles the
//! members of its recurrence before the row is stored, and the write that completes a
//! card that repeats makes the next card in the same transaction.
//!
//! A card repeats on a board that has a rule of done whose property it has, and names
//! one of its date properties as its due date (`dueDatePropertyId`; the first date
//! property the board falls back to without one is not enough). Setting a recurrence on a card of another board is refused
//! ([`BlockError::RecurrenceNeedsBoardRules`]), but for a write that tells nobody (a
//! copy, an import), which drops it: a copy never fails over it. The identifier of the
//! next card (`recurredTo`) is the server's: what a write sends is replaced by what is
//! stored, a new card has none, and setting the recurrence again takes it away, so that
//! the card repeats anew.
//!
//! The write that turns a card that repeats from not done to done hands its recurrence
//! to a new card and keeps the new card's identifier: the next card has the completed
//! one's title, icon, parent card, values and body, its value of the rule of done set
//! back ([`fvoci_model::not_done_value`]), its task items unchecked and its due date
//! moved on in the completer's time zone, counted from the first date of the chain
//! (`Recurrence::from`), which it is handed too. A change of how a card repeats or of
//! its due date starts a chain anew; a write that gives a recurrence back to a card
//! that has none (an undone completion) gives back its chain. It is made by the completer, and nobody is
//! told of whom it names: they were told of the card it follows. Its making is a line
//! of its activity (`created`). A template does not repeat; the next card is not made
//! where the board has lost its rules since.

use fvoci_model::{
    Block, BlockError, BlockType, Container, JsonMap, PropertyType, RECURRED_TO_FIELD,
    RECURRENCE_FIELD, Recurrence, TimeZone, is_card_done, new_id, not_done_value,
    with_tasks_unchecked,
};
use serde_json::Value;

use crate::blocks::{self, Written};
use crate::body_versions::BodyWrite;
use crate::db::{Id, Scope};
use crate::error::StoreError;
use crate::file_refs::NewBodyFiles;
use crate::notification_noting::Telling;
use crate::{card_values, containers};

/// The next card a write that completes a card makes, as [`settle_change`] plans it.
#[derive(Debug)]
pub(crate) struct NextCard {
    /// Its identifier, which the completed card names already.
    id: String,
    /// How it repeats: as the completed card did.
    recurrence: Recurrence,
    /// The value of the property of the rule of done before the card was done.
    before: Option<Value>,
    /// The board of both, as the write read it.
    board: Container,
}

/// Whether cards of `board` may repeat: it has a rule of done whose property it has,
/// and names a date property of its own as its due date.
fn repeats_on(board: &Container) -> bool {
    let named_due = board.due_date_property_id.as_deref().is_some_and(|due| {
        board.card_properties.iter().any(|definition| {
            definition.id == due && definition.property_type == PropertyType::Date
        })
    });
    named_due
        && board.done_when.as_ref().is_some_and(|rule| {
            board
                .card_properties
                .iter()
                .any(|definition| definition.id == rule.property_id)
        })
}

/// The values of the card's properties; none if its `properties` is no object.
fn values_of(block: &Block) -> JsonMap {
    match block.fields.get("properties") {
        Some(Value::Object(values)) => values.clone(),
        _ => JsonMap::new(),
    }
}

/// Refuses a recurrence set on a card of a board where cards do not repeat, or drops it
/// for a write that tells nobody.
fn refuse_or_drop(block: &mut Block, telling: Telling) -> Result<(), StoreError> {
    if matches!(telling, Telling::Nobody) {
        block.fields.remove(RECURRENCE_FIELD);
        return Ok(());
    }
    Err(StoreError::InvalidBlock {
        id: block.id.clone(),
        source: BlockError::RecurrenceNeedsBoardRules,
    })
}

/// Settles the members of the recurrence of a new block before it is stored: a card has
/// no next card yet, and repeats only on a board where cards may.
///
/// # Errors
///
/// Returns [`StoreError::InvalidBlock`] for a recurrence on a card of a board where
/// cards do not repeat, but for a write that tells nobody, and the store's error.
pub(crate) async fn settle_new(
    scope: &mut Scope<'_>,
    block: &mut Block,
    telling: Telling,
) -> Result<(), StoreError> {
    if block.block_type != BlockType::Card {
        return Ok(());
    }
    block.fields.remove(RECURRED_TO_FIELD);
    if Recurrence::of_fields(&block.fields).is_none() {
        block.fields.remove(RECURRENCE_FIELD);
        return Ok(());
    }
    let board = containers::get_container(scope, &block.container_id).await?;
    if !repeats_on(&board) {
        refuse_or_drop(block, telling)?;
    }
    Ok(())
}

/// Settles the members of the recurrence of a card that is changed from `existing`
/// before it is stored, and plans the next card if the change completes a card that
/// repeats: the card then names it and hands its recurrence to it.
///
/// # Errors
///
/// As [`settle_new`].
pub(crate) async fn settle_change(
    scope: &mut Scope<'_>,
    existing: &Block,
    block: &mut Block,
    telling: Telling,
) -> Result<Option<NextCard>, StoreError> {
    if block.block_type != BlockType::Card {
        return Ok(None);
    }
    match existing.fields.get(RECURRED_TO_FIELD) {
        Some(next) => block
            .fields
            .insert(RECURRED_TO_FIELD.to_owned(), next.clone()),
        None => block.fields.remove(RECURRED_TO_FIELD),
    };
    let Some(recurrence) = Recurrence::of_fields(&block.fields) else {
        block.fields.remove(RECURRENCE_FIELD);
        return Ok(None);
    };
    let board = containers::get_container(scope, &block.container_id).await?;
    let had = Recurrence::of_fields(&existing.fields);
    if had.is_none_or(|had| !had.same_rule(recurrence)) {
        if !repeats_on(&board) {
            refuse_or_drop(block, telling)?;
            return Ok(None);
        }
        // Set again, it repeats anew: whatever card it made before is another's.
        block.fields.remove(RECURRED_TO_FIELD);
    }
    let due_changed = board
        .effective_due_date_property_id()
        .is_some_and(|due| values_of(existing).get(due) != values_of(block).get(due));
    // A new due date starts a chain anew.
    let recurrence = if due_changed {
        recurrence.anew()
    } else {
        chained(had, recurrence)
    };
    block
        .fields
        .insert(RECURRENCE_FIELD.to_owned(), recurrence.to_value());
    if block.is_template() || !repeats_on(&board) {
        return Ok(None);
    }
    let done = |card: &Block| {
        is_card_done(
            &values_of(card),
            &board.card_properties,
            board.done_when.as_ref(),
        )
    };
    if done(existing) || !done(block) {
        return Ok(None);
    }
    let before = board
        .done_when
        .as_ref()
        .and_then(|rule| values_of(existing).get(&rule.property_id).cloned());
    let id = new_id();
    block.fields.remove(RECURRENCE_FIELD);
    block
        .fields
        .insert(RECURRED_TO_FIELD.to_owned(), Value::from(id.as_str()));
    Ok(Some(NextCard {
        id,
        recurrence,
        before,
        board,
    }))
}

/// How a card whose due date stays repeats (`sent`), as for the first date of its
/// chain: the one it had while it repeats as it did (`had`), the one a write gives back
/// to a card that did not repeat (an undone completion), and none once how it repeats
/// is changed.
fn chained(had: Option<Recurrence>, sent: Recurrence) -> Recurrence {
    match had {
        Some(had) if had.same_rule(sent) => Recurrence {
            from: had.from,
            ..sent
        },
        Some(_) => sent.anew(),
        None => sent,
    }
}

/// Makes the card that follows `completed`, as [`settle_change`] planned it, by
/// `user_id`, once `completed` is stored.
///
/// # Errors
///
/// Returns the store's error; the write of the completed card fails with it.
pub(crate) async fn make_next(
    scope: &mut Scope<'_>,
    completed: &Block,
    next: NextCard,
    user_id: &str,
) -> Result<(), StoreError> {
    let NextCard {
        id,
        recurrence,
        before,
        board,
    } = next;
    let time_zone = time_zone_of(scope, user_id).await?;
    let mut values = values_of(completed);
    if let Some(rule) = &board.done_when
        && let Some(definition) = board
            .card_properties
            .iter()
            .find(|definition| definition.id == rule.property_id)
    {
        let current = values.get(&rule.property_id);
        match not_done_value(definition, rule, current, before.as_ref()) {
            Some(value) => values.insert(rule.property_id.clone(), value),
            None => values.remove(&rule.property_id),
        };
    }
    let mut handed = recurrence.anew();
    if let Some(due) = board.effective_due_date_property_id() {
        handed = recurrence.handed_on(values.get(due), &time_zone);
        match recurrence.next_due(values.get(due), scope.now_millis(), &time_zone) {
            Some(value) => values.insert(due.to_owned(), value),
            None => values.remove(due),
        };
    }
    let mut fields = JsonMap::new();
    if let Some(icon) = completed.fields.get("icon") {
        fields.insert("icon".to_owned(), icon.clone());
    }
    fields.insert("properties".to_owned(), Value::Object(values));
    fields.insert(RECURRENCE_FIELD.to_owned(), handed.to_value());
    let mut cards = [Block {
        id,
        container_id: completed.container_id.clone(),
        parent_id: completed.parent_id.clone(),
        block_type: BlockType::Card,
        title: completed.title.clone(),
        fields,
        body: completed.body.as_ref().map(with_tasks_unchecked),
        ..Block::default()
    }];
    card_values::clear_copied_values(&mut cards, &board.card_properties);
    card_values::keep_cards_of_board(scope, &mut cards, &board.id, &board.card_properties).await?;
    let [mut card] = cards;
    // The files of its body are the completed card's, of the same board.
    let how = Written {
        new_files: NewBodyFiles::Unchecked,
        body_write: BodyWrite::Whole,
        telling: Telling::Nobody,
    };
    blocks::insert_block_at(scope, &mut card, user_id, None, how).await?;
    Ok(())
}

/// The time zone of `user_id`'s settings; the device's (UTC to the server) for nobody
/// and for an account without settings.
async fn time_zone_of(scope: &mut Scope<'_>, user_id: &str) -> Result<TimeZone, StoreError> {
    if user_id.is_empty() {
        return Ok(TimeZone::default());
    }
    let mut sql = scope.sql("SELECT time_zone FROM user_settings WHERE user_id = ");
    sql.bind(Id(user_id));
    let name: Option<String> = scope.first(sql).await?;
    Ok(name.and_then(|name| name.parse().ok()).unwrap_or_default())
}
