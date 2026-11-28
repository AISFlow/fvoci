//! The blocks that go into the trash, held before what they keep is read for it.
//!
//! A reaction is added under a hold of its block's row alone
//! ([`crate::Store::add_reaction`]), not of the board. One added between the reading of
//! the block's reactions for the trash and the deletion of the block's row would go
//! with the row and be missing from the trash. So the rows of the blocks are held first,
//! in the order of their identifiers ([`LockRank::Block`]): an addition under way is
//! waited for and read with the others, and a later one finds no block.
//!
//! A transaction that puts several trees or boards in the trash holds all of their
//! blocks before the first of them ([`hold_trees_of`], [`hold_blocks_of_containers`]):
//! each deletion then holds again only what it holds already.

use std::collections::BTreeSet;

use crate::db::{Dialect, Id, LockRank, RowLock, Scope};
use crate::error::StoreError;

/// How many identifiers one statement binds.
const BATCH: usize = 300;

/// Holds the rows of the blocks `ids` against any change, in the order of their
/// identifiers. One that is not there is not held.
pub(crate) async fn hold_blocks(scope: &mut Scope<'_>, ids: &[&str]) -> Result<(), StoreError> {
    let ordered: Vec<&str> = ids
        .iter()
        .copied()
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .collect();
    for batch in ordered.chunks(BATCH) {
        let mut sql = scope.sql("SELECT id FROM blocks WHERE id IN (");
        sql.bind_list(batch.iter().map(|id| Id(id)))
            .push(") ORDER BY id");
        let clause = scope.rows_lock(LockRank::Block, batch.iter().copied(), RowLock::Update);
        if scope.dialect() == Dialect::Postgres {
            sql.push_lock(clause);
            scope.fetch_all(sql).await?;
        }
    }
    Ok(())
}

/// Holds the blocks of `block_ids` and everything below them as [`hold_blocks`] does.
pub(crate) async fn hold_trees_of(
    scope: &mut Scope<'_>,
    block_ids: &[String],
) -> Result<(), StoreError> {
    let mut ids: Vec<String> = Vec::new();
    for batch in block_ids.chunks(BATCH) {
        let mut sql = scope.sql("WITH RECURSIVE tree(id) AS (SELECT id FROM blocks WHERE id IN (");
        sql.bind_list(batch.iter().map(|id| Id(id))).push(
            ") UNION SELECT b.id FROM blocks AS b JOIN tree ON b.parent_id = tree.id) \
             SELECT id FROM tree",
        );
        let found: Vec<String> = scope.all(sql).await?;
        ids.extend(found);
    }
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    hold_blocks(scope, &ids).await
}

/// Holds every block of the boards `container_ids` as [`hold_blocks`] does.
pub(crate) async fn hold_blocks_of_containers(
    scope: &mut Scope<'_>,
    container_ids: &[&str],
) -> Result<(), StoreError> {
    let mut ids: Vec<String> = Vec::new();
    for batch in container_ids.chunks(BATCH) {
        let mut sql = scope.sql("SELECT id FROM blocks WHERE container_id IN (");
        sql.bind_list(batch.iter().map(|id| Id(id))).push(")");
        let found: Vec<String> = scope.all(sql).await?;
        ids.extend(found);
    }
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    hold_blocks(scope, &ids).await
}
