//! The numbers of the cards of a board, as `#12` names the twelfth card made on it.
//!
//! A board's row holds the number its next card gets (`containers.next_card_number`)
//! and a card's row its number (`blocks.number`, which no other card of the board has).
//! Every block is stored by one write ([`crate::blocks`]), which holds its board from
//! before it reads anything ([`crate::containers::lock_container`]): there a block that
//! takes a number ([`Block::takes_number`]) and has none yet takes the board's next,
//! so two writes at once never take one number. The next number only goes up, so a
//! number is never given again, also once its card is gone for good. The trash keeps a
//! card's number with the card and a board's next number with the board.

use fvoci_model::{Block, BlockType};

use crate::Store;
use crate::blocks::{fetch_blocks, select_blocks};
use crate::db::{Id, Param, Row, Scope};
use crate::error::StoreError;

impl Store {
    /// The card of the board that has `number`, if there is one: none in the trash.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and [`StoreError::Corrupt`]
    /// if the stored block cannot be read.
    pub async fn get_card_by_number(
        &self,
        container_id: &str,
        number: u32,
    ) -> Result<Option<Block>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_blocks(scope);
            sql.push_bind(" WHERE container_id = ", Id(container_id))
                .push_bind(" AND number = ", i64::from(number));
            let found = fetch_blocks(scope, sql).await?.into_iter().next();
            Ok(found.filter(Block::takes_number))
        })
        .await
    }
}

/// Gives `block` the number it is stored with: that of the block as it was stored,
/// `stored` (`None` for a new block); the board's next one if that had none and the
/// block takes one; for a card template, whatever number `stored` already held (so
/// turning an ordinary card into a template preserves its number in storage and turning
/// it back restores it without consuming a new one, while a template created from
/// scratch stays unnumbered); and none for any other block. What the block came with is
/// not asked: a new block, also a copy or an import, is a new card. Its board is held.
pub(crate) async fn number_written(
    scope: &mut Scope<'_>,
    block: &mut Block,
    stored: Option<&Block>,
) -> Result<(), StoreError> {
    block.number = if block.takes_number() {
        match stored.and_then(|stored| stored.number) {
            Some(number) => Some(number),
            None => Some(take_next(scope, &block.container_id).await?),
        }
    } else if block.block_type == BlockType::Card && block.is_template() {
        stored.and_then(|stored| stored.number)
    } else {
        None
    };
    Ok(())
}

/// Gives the blocks of one deletion that come back from the trash the numbers they
/// come back with: each its own (including a preserved number on a card template), and
/// the board's next ones, in the order they were made, to the ordinary cards that went
/// to the trash before cards had numbers. Their board is held.
pub(crate) async fn number_restored(
    scope: &mut Scope<'_>,
    restored: &mut [Block],
) -> Result<(), StoreError> {
    let mut unnumbered: Vec<&mut Block> = Vec::new();
    for block in restored.iter_mut() {
        if block.takes_number() {
            if block.number.is_none() {
                unnumbered.push(block);
            }
        } else if !(block.block_type == BlockType::Card && block.is_template()) {
            block.number = None;
        }
    }
    unnumbered.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    for block in unnumbered {
        block.number = Some(take_next(scope, &block.container_id).await?);
    }
    Ok(())
}

/// Keeps the next number of a board that goes to the trash with it, in the row the
/// trash keeps it in.
pub(crate) async fn keep_with_trashed(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE deleted_containers SET next_card_number = ");
    sql.push_bind(
        "(SELECT next_card_number FROM containers WHERE id = ",
        Id(container_id),
    )
    .push_bind(") WHERE id = ", Id(container_id));
    scope.execute(sql).await?;
    Ok(())
}

/// Gives a board that comes back from the trash, its row stored again, the next number
/// it went with.
pub(crate) async fn bring_back(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE containers SET next_card_number = ");
    sql.changing_no_access()
        .push_bind(
            "(SELECT next_card_number FROM deleted_containers WHERE id = ",
            Id(container_id),
        )
        .push_bind(") WHERE id = ", Id(container_id));
    scope.execute(sql).await?;
    Ok(())
}

/// The number of a block as a statement binds it.
pub(crate) fn number_param(number: Option<u32>) -> Param {
    Param::OptInt(number.map(i64::from))
}

/// The number of a block read from `blocks.number` at `index`.
pub(crate) fn number_from_row(row: &Row, index: usize) -> Result<Option<u32>, StoreError> {
    row.opt_int(index)?.map(number_from).transpose()
}

/// Takes the next number of a board, which is held, and moves it on. No check of
/// access reads it, so a card that is made keeps the permissions that are cached.
async fn take_next(scope: &mut Scope<'_>, container_id: &str) -> Result<u32, StoreError> {
    let mut sql =
        scope.sql("UPDATE containers SET next_card_number = next_card_number + 1 WHERE id = ");
    sql.bind(Id(container_id))
        .push(" RETURNING next_card_number - 1")
        .changing_no_access();
    let row = scope.fetch_optional(sql).await?.ok_or_else(|| {
        StoreError::not_found(format_args!("board ID={container_id} of a numbered card"))
    })?;
    number_from(row.int(0)?)
}

/// A number the store wrote, read back.
fn number_from(number: i64) -> Result<u32, StoreError> {
    u32::try_from(number)
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| StoreError::Corrupt {
            column: "number",
            reason: format!("{number} is no card number"),
        })
}
