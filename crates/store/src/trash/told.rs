//! What people were told of what goes for good: the notifications about a card or a page
//! (`notifications`), and the rows of the list of whom to tell of it
//! (`notification_outbox`), go with it, found by the block or by the board they name.

use crate::db::{Id, Scope, SqlFragment};
use crate::error::StoreError;

/// How many identifiers one statement binds.
const BATCH: usize = 300;

/// What went for good.
#[derive(Debug, Clone, Copy)]
pub(super) enum Gone<'a> {
    /// Blocks of the trash, by identifier.
    Blocks(&'a [String]),
    /// Boards and wikis of the trash, by identifier.
    Containers(&'a [String]),
}

/// Forgets what was told of what is `gone`. A card or a page that is somewhere still,
/// among the blocks or in the trash (moved off the board, or deleted on its own and
/// kept there), keeps what was told of it.
pub(super) async fn forget_told_of(
    scope: &mut Scope<'_>,
    gone: Gone<'_>,
) -> Result<(), StoreError> {
    let (column, ids) = match gone {
        Gone::Blocks(ids) => ("block_id", ids),
        Gone::Containers(ids) => ("container_id", ids),
    };
    for table in ["notifications", "notification_outbox"] {
        let mut somewhere = SqlFragment::of(&[
            " AND NOT EXISTS (SELECT 1 FROM blocks AS b WHERE b.id = ",
            table,
            ".block_id",
        ]);
        somewhere
            .push_fragment(&scope.same_organization(&[("b", table)]))
            .push(") AND NOT EXISTS (SELECT 1 FROM deleted_blocks AS d WHERE d.id = ")
            .push(table)
            .push(".block_id")
            .push_fragment(&scope.same_organization(&[("d", table)]))
            .push(")");
        for batch in ids.chunks(BATCH) {
            let mut sql = scope.sql(["DELETE FROM ", table, " WHERE ", column, " IN ("]);
            sql.bind_list(batch.iter().map(|id| Id(id)))
                .push(")")
                .push(&somewhere);
            scope.execute(sql).await?;
        }
    }
    Ok(())
}
