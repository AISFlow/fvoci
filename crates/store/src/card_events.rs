//! The activity of cards (`card_events`): who made a card, changed its title, the value
//! of one of its properties, its parent card or whether it is a template, or brought it
//! back from the trash, and when ([`fvoci_model::CardEvent`]).
//!
//! Every write of a block passes through [`crate::blocks`], which notes here what it
//! made or changed of a card in the write's transaction: a new card in its own line, a
//! change in a line for each thing that changed ([`fvoci_model::card_changes`]). A copy,
//! a card made from a template, an import and the next card of one that repeats are
//! new cards, each its line `created`; what such a write changes of a card that was
//! there, which says what was said before, is not noted.
//!
//! The same person changing the same thing of a card again within
//! [`MERGE_WITHIN_MILLIS`], with nobody else's change of it between, changes the line
//! of the first change rather than adding one, and a change back to what it was takes
//! the line away: a number saved as it is typed leaves one line. A card keeps
//! [`KEPT_PER_CARD`] lines, the newest.
//!
//! A line hangs on no block by a key, as the versions of a body do: the lines of a card
//! in the trash stay and go on when it comes back, and go when it is gone for good,
//! with its board, or with its organization ([`forget_gone_cards`]). The erasure of an
//! account leaves the lines it did without its identifier and its token's name
//! ([`forget_actor`]).

use fvoci_model::{Block, BlockType, CardChange, CardEvent, CardEventKind, card_changes, new_id};
use serde_json::Value;

use crate::Store;
use crate::blocks::Paging;
use crate::db::{Id, OptId, Param, Row, Scope, SqlFragment};
use crate::error::StoreError;
use crate::notification_noting::Telling;

/// How long after a line its person's next change of the same thing joins it, in
/// milliseconds: five minutes, the time a field edited on is saved over and over in.
pub(crate) const MERGE_WITHIN_MILLIS: i64 = 5 * 60 * 1000;

/// How many lines a card keeps, the newest: past it the oldest go. Five hundred is more
/// than a card's life on a board of a team usually has, and bounds the table.
pub(crate) const KEPT_PER_CARD: i64 = 500;

/// The columns of a line as [`event_of`] reads them.
const COLUMNS: &str = "id, kind, property_id, before_value, after_value, actor_id, \
                       actor_token_name, created_at, updated_at";

/// Who did a write, as its lines say it: the person (none for a write of nobody's,
/// which a copy by the server is) and the token an agent wrote through.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Actor {
    id: Option<String>,
    token_name: Option<String>,
}

impl Actor {
    fn of(scope: &Scope<'_>, user_id: &str) -> Self {
        Self {
            id: (!user_id.is_empty()).then(|| user_id.to_owned()),
            token_name: scope.agent().map(|agent| agent.token_name.to_owned()),
        }
    }
}

/// Notes that `block`, which was just stored new, was made, if it is a card.
pub(crate) async fn note_created(
    scope: &mut Scope<'_>,
    block: &Block,
    user_id: &str,
) -> Result<(), StoreError> {
    if block.block_type != BlockType::Card {
        return Ok(());
    }
    let actor = Actor::of(scope, user_id);
    let at = block.created_at;
    insert(scope, block, &actor, CardEventKind::Created, at).await
}

/// Notes what a write of the card `block` changed of it, which was `existing`, unless
/// the write tells nobody ([`Telling::Nobody`]): a copy's or an import's, which says
/// what was said before.
pub(crate) async fn note_changes(
    scope: &mut Scope<'_>,
    existing: &Block,
    block: &Block,
    user_id: &str,
    telling: Telling,
) -> Result<(), StoreError> {
    if block.block_type != BlockType::Card || matches!(telling, Telling::Nobody) {
        return Ok(());
    }
    let changes = card_changes(existing, block);
    if changes.is_empty() {
        return Ok(());
    }
    let (actor, now) = (Actor::of(scope, user_id), block.updated_at);
    for change in changes {
        note_change(scope, block, &actor, change, now).await?;
    }
    Ok(())
}

/// Notes that the cards among `blocks`, which just came back from the trash, did so.
pub(crate) async fn note_restored(
    scope: &mut Scope<'_>,
    blocks: &[Block],
    user_id: &str,
    now: i64,
) -> Result<(), StoreError> {
    let actor = Actor::of(scope, user_id);
    for block in blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Card)
    {
        insert(scope, block, &actor, CardEventKind::Restored, now).await?;
        keep_the_newest(scope, &block.id).await?;
    }
    Ok(())
}

/// Notes one change: in the line of the same person's last change of the same thing
/// if that is the card's last line of it and was written within
/// [`MERGE_WITHIN_MILLIS`], taking the line away if the change takes the thing back to
/// what that line says it was; in a new line otherwise.
async fn note_change(
    scope: &mut Scope<'_>,
    block: &Block,
    actor: &Actor,
    change: CardChange,
    now: i64,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "SELECT id, actor_id, actor_token_name, before_value, updated_at FROM card_events \
         WHERE card_id = ",
    );
    sql.bind(Id(&block.id))
        .push_bind(" AND kind = ", change.kind.as_str());
    match &change.property_id {
        Some(property_id) => sql.push_bind(" AND property_id = ", property_id.as_str()),
        None => sql.push(" AND property_id IS NULL"),
    };
    sql.push(" ORDER BY updated_at DESC, id DESC LIMIT 1");
    if let Some(last) = scope.fetch_optional(sql).await? {
        let (id, actor_id, token_name) = (last.id(0)?, last.id(1)?, last.opt_text(2)?);
        let same_actor =
            actor.id.as_deref().unwrap_or_default() == actor_id && actor.token_name == token_name;
        if same_actor && now.saturating_sub(last.int(4)?) <= MERGE_WITHIN_MILLIS {
            let back = last.json(3, "before_value")? == change.after;
            let mut sql = if back {
                scope.sql("DELETE FROM card_events WHERE id = ")
            } else {
                let mut sql = scope.sql("UPDATE card_events SET after_value = ");
                sql.bind(json_param(&change.after)?)
                    .push_bind(", updated_at = ", now)
                    .push(" WHERE id = ");
                sql
            };
            // With its card, so that PostgreSQL finds the line by the card's index: its
            // key leads with the organization, which the wall asks of every row.
            sql.changing_no_access()
                .bind(Id(&id))
                .push_bind(" AND card_id = ", Id(&block.id));
            scope.execute(sql).await?;
            return Ok(());
        }
    }
    let mut sql = scope.sql(
        "INSERT INTO card_events (id, container_id, card_id, actor_id, actor_token_name, \
         kind, property_id, before_value, after_value, created_at, updated_at) VALUES (",
    );
    sql.changing_no_access()
        .bind(Id(&new_id()))
        .push_bind(", ", Id(&block.container_id))
        .push_bind(", ", Id(&block.id))
        .push_bind(", ", OptId(actor.id.as_deref().unwrap_or_default()))
        .push_bind(", ", Param::OptText(actor.token_name.clone()))
        .push_bind(", ", change.kind.as_str())
        .push_bind(", ", Param::OptText(change.property_id))
        .push_bind(", ", json_param(&change.before)?)
        .push_bind(", ", json_param(&change.after)?)
        .push_bind(", ", now)
        .push_bind(", ", now)
        .push(")");
    scope.execute(sql).await?;
    keep_the_newest(scope, &block.id).await
}

/// Writes a line of `kind` with no values, `created` or `restored`, at `at`: no other
/// line joins it. A card made has no line before it.
async fn insert(
    scope: &mut Scope<'_>,
    block: &Block,
    actor: &Actor,
    kind: CardEventKind,
    at: i64,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "INSERT INTO card_events (id, container_id, card_id, actor_id, actor_token_name, \
         kind, property_id, before_value, after_value, created_at, updated_at) VALUES (",
    );
    sql.changing_no_access()
        .bind(Id(&new_id()))
        .push_bind(", ", Id(&block.container_id))
        .push_bind(", ", Id(&block.id))
        .push_bind(", ", OptId(actor.id.as_deref().unwrap_or_default()))
        .push_bind(", ", Param::OptText(actor.token_name.clone()))
        .push_bind(", ", kind.as_str())
        .push(", NULL, NULL, NULL")
        .push_bind(", ", at)
        .push_bind(", ", at)
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// A value of a line as it is stored: SQL `NULL` for none.
fn json_param(value: &Value) -> Result<Param, StoreError> {
    Param::opt_jsonb((!value.is_null()).then_some(value))
}

/// Takes away the lines of the card past the [`KEPT_PER_CARD`] newest.
async fn keep_the_newest(scope: &mut Scope<'_>, card_id: &str) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT updated_at, id FROM card_events WHERE card_id = ");
    sql.bind(Id(card_id))
        .push(" ORDER BY updated_at DESC, id DESC LIMIT 1 OFFSET ")
        .bind(KEPT_PER_CARD);
    let Some(first_gone) = scope.fetch_optional(sql).await? else {
        return Ok(());
    };
    let (at, id) = (first_gone.int(0)?, first_gone.id(1)?);
    let mut sql = scope.sql("DELETE FROM card_events WHERE card_id = ");
    sql.changing_no_access()
        .bind(Id(card_id))
        .push_bind(" AND (updated_at < ", at)
        .push_bind(" OR (updated_at = ", at)
        .push_bind(" AND id <= ", Id(&id))
        .push("))");
    scope.execute(sql).await?;
    Ok(())
}

/// Forgets the lines of cards that are gone for good, neither among the blocks nor in
/// the trash: where the versions of their bodies go ([`crate::block_derived`]).
pub(crate) async fn forget_gone_cards(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut statement = SqlFragment::of(&[
        "DELETE FROM card_events WHERE NOT EXISTS (SELECT 1 FROM blocks b \
         WHERE b.id = card_events.card_id",
    ]);
    statement
        .push_fragment(&scope.same_organization(&[("b", "card_events")]))
        .push(
            ") AND NOT EXISTS (SELECT 1 FROM deleted_blocks d \
             WHERE d.id = card_events.card_id",
        )
        .push_fragment(&scope.same_organization(&[("d", "card_events")]))
        .push(")");
    let mut sql = scope.sql(&statement);
    sql.changing_no_access();
    scope.execute(sql).await?;
    Ok(())
}

/// Leaves the lines `user_id` did, in every organization the scope reaches, without
/// who did them: the identifier and the name of the token go, the lines stay. For the
/// last step of the erasure of the account, whose row stays as an identifier alone.
pub(crate) async fn forget_actor(scope: &mut Scope<'_>, user_id: &str) -> Result<(), StoreError> {
    let mut sql = scope
        .sql("UPDATE card_events SET actor_id = NULL, actor_token_name = NULL WHERE actor_id = ");
    sql.changing_no_access().bind(Id(user_id));
    scope.execute(sql).await?;
    Ok(())
}

/// Reads a line as [`COLUMNS`] has it.
fn event_of(row: &Row) -> Result<CardEvent, StoreError> {
    let kind = row.text(1)?;
    let kind = kind.parse().map_err(|_| StoreError::Corrupt {
        column: "card_events.kind",
        reason: format!("`{kind}` is no kind of a card's activity"),
    })?;
    let nonempty = |text: String| (!text.is_empty()).then_some(text);
    Ok(CardEvent {
        id: row.id(0)?,
        kind,
        property_id: row.opt_text(2)?,
        before: row.json(3, "card_events.before_value")?,
        after: row.json(4, "card_events.after_value")?,
        actor_id: nonempty(row.id(5)?),
        actor_token_name: row.opt_text(6)?,
        created_at: row.int(7)?,
        updated_at: row.int(8)?,
    })
}

impl Store {
    /// One page of the activity of the card `card_id`, the newest first: by when each
    /// line was last written, then by its identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the lines cannot be read, and
    /// [`StoreError::Corrupt`] if one cannot be read as a line.
    pub async fn card_events(
        &self,
        card_id: &str,
        paging: Paging,
    ) -> Result<Vec<CardEvent>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(["SELECT ", COLUMNS, " FROM card_events WHERE card_id = "]);
            sql.bind(Id(card_id))
                .push(" ORDER BY updated_at DESC, id DESC LIMIT ")
                .bind(i64::from(paging.per_page.get()))
                .push_bind(" OFFSET ", paging.offset());
            scope.fetch_all(sql).await?.iter().map(event_of).collect()
        })
        .await
    }
}
